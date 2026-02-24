use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

const BUNDLED_SKIN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/skins");
const HOMEBREW_CELLAR_SKIN_DIR: &str =
    "/opt/homebrew/Cellar/midnight-commander/4.8.33/share/mc/skins";
const HOMEBREW_PREFIX_SKIN_DIR: &str = "/opt/homebrew/share/mc/skins";
const LOCAL_SKIN_DIR: &str = "/usr/local/share/mc/skins";
const SYSTEM_SKIN_DIR: &str = "/usr/share/mc/skins";

static ACTIVE_SKIN: OnceLock<RwLock<Arc<UiSkin>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct UiSkin {
    name: String,
    styles: HashMap<String, HashMap<String, Style>>,
    panel_border_set: border::Set,
    dialog_border_set: border::Set,
}

impl UiSkin {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn panel_border_set(&self) -> border::Set {
        self.panel_border_set
    }

    pub fn dialog_border_set(&self) -> border::Set {
        self.dialog_border_set
    }

    pub fn style(&self, section: &str, key: &str) -> Style {
        let section_key = section.to_ascii_lowercase();
        let key = key.to_ascii_lowercase();
        let Some(section_styles) = self.styles.get(&section_key) else {
            return Style::default();
        };

        let default = section_styles.get("_default_").copied().unwrap_or_default();
        if key == "_default_" {
            return default;
        }

        section_styles
            .get(&key)
            .copied()
            .map(|specific| default.patch(specific))
            .unwrap_or(default)
    }

    fn fallback() -> Self {
        Self::from_ini("default", include_str!("../assets/skins/default.ini"))
            .unwrap_or_else(|_| Self::hardcoded_fallback())
    }

    fn hardcoded_fallback() -> Self {
        let mut core = HashMap::new();
        core.insert("_default_".to_string(), Style::default().fg(Color::Gray));
        core.insert("selected".to_string(), Style::default().fg(Color::Yellow));
        core.insert("marked".to_string(), Style::default().fg(Color::Yellow));
        core.insert("markselect".to_string(), Style::default().fg(Color::Yellow));
        core.insert("header".to_string(), Style::default().fg(Color::Yellow));

        let mut statusbar = HashMap::new();
        statusbar.insert(
            "_default_".to_string(),
            Style::default().fg(Color::DarkGray),
        );

        let mut dialog = HashMap::new();
        dialog.insert("_default_".to_string(), Style::default().fg(Color::Gray));
        dialog.insert("dfocus".to_string(), Style::default().fg(Color::Yellow));
        dialog.insert("dtitle".to_string(), Style::default().fg(Color::Yellow));

        let mut styles = HashMap::new();
        styles.insert("core".to_string(), core);
        styles.insert("statusbar".to_string(), statusbar);
        styles.insert("dialog".to_string(), dialog);
        styles.insert("viewer".to_string(), HashMap::new());
        styles.insert("filehighlight".to_string(), HashMap::new());

        Self {
            name: String::from("fallback"),
            styles,
            panel_border_set: border::PLAIN,
            dialog_border_set: border::PLAIN,
        }
    }

    fn from_named_skin(name: &str, skin_dir: Option<&Path>) -> Result<Self, String> {
        let path = resolve_skin_path(name, skin_dir).ok_or_else(|| {
            let mut searched = search_dirs(skin_dir);
            searched.sort();
            searched.dedup();
            format!("unable to locate skin '{name}' in: {}", searched.join(", "))
        })?;

        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read skin '{}': {error}", path.display()))?;
        let skin_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(name)
            .to_string();
        Self::from_ini(&skin_name, &source)
    }

    fn from_ini(name: &str, source: &str) -> Result<Self, String> {
        let sections = parse_ini(source);
        let aliases = sections.get("aliases").cloned().unwrap_or_default();
        let styles = parse_styles(&sections, &aliases);
        let lines = sections.get("lines").cloned().unwrap_or_default();
        let panel_border_set = parse_border_set(&lines, false);
        let dialog_border_set = parse_border_set(&lines, true);

        if styles.is_empty() {
            return Err(format!("skin '{name}' does not define style sections"));
        }

        Ok(Self {
            name: name.to_string(),
            styles,
            panel_border_set,
            dialog_border_set,
        })
    }
}

pub fn configure_skin(name: &str, skin_dir: Option<&Path>) -> Result<(), String> {
    let skin = Arc::new(UiSkin::from_named_skin(name, skin_dir)?);
    let store = ACTIVE_SKIN.get_or_init(|| RwLock::new(Arc::new(UiSkin::fallback())));
    let mut guard = store
        .write()
        .map_err(|_| String::from("skin store lock poisoned"))?;
    *guard = skin;
    Ok(())
}

pub fn current_skin() -> Arc<UiSkin> {
    let store = ACTIVE_SKIN.get_or_init(|| RwLock::new(Arc::new(UiSkin::fallback())));
    store
        .read()
        .map(|guard| Arc::clone(&guard))
        .unwrap_or_else(|_| Arc::new(UiSkin::fallback()))
}

pub fn current_skin_name() -> String {
    current_skin().name().to_string()
}

pub fn list_available_skins(skin_dir: Option<&Path>) -> Vec<String> {
    let mut names = BTreeSet::new();

    for directory in search_dirs(skin_dir) {
        let path = PathBuf::from(directory);
        let Ok(entries) = fs::read_dir(path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
                continue;
            };
            if !extension.eq_ignore_ascii_case("ini") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                names.insert(stem.to_string());
            }
        }
    }

    names.into_iter().collect()
}

fn resolve_skin_path(name: &str, skin_dir: Option<&Path>) -> Option<PathBuf> {
    let requested_path = Path::new(name);
    if requested_path.is_absolute() || name.contains('/') {
        if requested_path.exists() {
            return Some(requested_path.to_path_buf());
        }
        let with_extension = requested_path.with_extension("ini");
        if with_extension.exists() {
            return Some(with_extension);
        }
    }

    let file_name = if name.ends_with(".ini") {
        name.to_string()
    } else {
        format!("{name}.ini")
    };
    for directory in search_dirs(skin_dir) {
        let path = Path::new(&directory).join(&file_name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn search_dirs(skin_dir: Option<&Path>) -> Vec<String> {
    let mut dirs = Vec::new();
    if let Some(path) = skin_dir {
        dirs.push(path.to_string_lossy().into_owned());
    }
    dirs.push(BUNDLED_SKIN_DIR.to_string());
    dirs.push(HOMEBREW_CELLAR_SKIN_DIR.to_string());
    dirs.push(HOMEBREW_PREFIX_SKIN_DIR.to_string());
    dirs.push(LOCAL_SKIN_DIR.to_string());
    dirs.push(SYSTEM_SKIN_DIR.to_string());
    dirs
}

fn parse_ini(source: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section = String::new();

    for raw_line in source.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            sections.entry(current_section.clone()).or_default();
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if current_section.is_empty() {
            continue;
        }

        sections
            .entry(current_section.clone())
            .or_default()
            .insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    sections
}

fn parse_styles(
    sections: &HashMap<String, HashMap<String, String>>,
    aliases: &HashMap<String, String>,
) -> HashMap<String, HashMap<String, Style>> {
    let mut styles = HashMap::new();

    for (section, entries) in sections {
        if section == "skin"
            || section == "lines"
            || section == "aliases"
            || section.starts_with("widget-")
        {
            continue;
        }

        let mut parsed_entries = HashMap::new();
        for (key, value) in entries {
            parsed_entries.insert(key.clone(), parse_style_spec(value, aliases));
        }

        if !parsed_entries.is_empty() {
            styles.insert(section.clone(), parsed_entries);
        }
    }

    styles
}

fn parse_style_spec(spec: &str, aliases: &HashMap<String, String>) -> Style {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Style::default();
    }

    let parts: Vec<&str> = trimmed.splitn(3, ';').collect();
    let (fg_part, bg_part, attr_part) = match parts.as_slice() {
        [fg] => (*fg, "", ""),
        [fg, bg] => (*fg, *bg, ""),
        [fg, bg, attrs] => (*fg, *bg, *attrs),
        _ => ("", "", ""),
    };

    let mut style = Style::default();

    if let Some(fg) = parse_color_token(fg_part, aliases, &mut HashSet::new()) {
        style = style.fg(fg);
    }
    if let Some(bg) = parse_color_token(bg_part, aliases, &mut HashSet::new()) {
        style = style.bg(bg);
    }

    for raw_attr in attr_part
        .split(['+', ',', '|'])
        .flat_map(|value| value.split_whitespace())
    {
        match raw_attr.trim().to_ascii_lowercase().as_str() {
            "bold" => style = style.add_modifier(Modifier::BOLD),
            "underline" => style = style.add_modifier(Modifier::UNDERLINED),
            "italic" => style = style.add_modifier(Modifier::ITALIC),
            "reverse" | "inverse" => style = style.add_modifier(Modifier::REVERSED),
            _ => {}
        }
    }

    style
}

fn parse_color_token(
    token: &str,
    aliases: &HashMap<String, String>,
    visiting: &mut HashSet<String>,
) -> Option<Color> {
    let normalized = token.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    if visiting.insert(normalized.clone())
        && let Some(alias_value) = aliases.get(&normalized)
        && let Some(color) = parse_color_token(alias_value, aliases, visiting)
    {
        return Some(color);
    }

    parse_direct_color(&normalized)
}

fn parse_direct_color(token: &str) -> Option<Color> {
    match token {
        "default" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "brown" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "lightgray" | "lightgrey" => Some(Color::Gray),
        "gray" | "grey" => Some(Color::DarkGray),
        "brightred" => Some(Color::LightRed),
        "brightgreen" => Some(Color::LightGreen),
        "yellow" => Some(Color::LightYellow),
        "brightblue" => Some(Color::LightBlue),
        "brightmagenta" => Some(Color::LightMagenta),
        "brightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => {
            if let Some(index) = token
                .strip_prefix("color")
                .and_then(|raw| raw.parse::<u8>().ok())
            {
                return Some(Color::Indexed(index));
            }

            if let Some(value) = token.strip_prefix("rgb")
                && value.len() == 3
            {
                let digits: Option<Vec<u8>> = value
                    .chars()
                    .map(|ch| {
                        ch.to_digit(10)
                            .and_then(|digit| (digit <= 5).then_some(digit as u8))
                    })
                    .collect();
                if let Some(digits) = digits {
                    let index = 16 + 36 * digits[0] + 6 * digits[1] + digits[2];
                    return Some(Color::Indexed(index));
                }
            }

            if let Some(shade) = token
                .strip_prefix("gray")
                .and_then(|raw| raw.parse::<u8>().ok())
            {
                return Some(Color::Indexed(232 + shade.min(23)));
            }

            if token.starts_with('#') {
                return parse_hex_color(token);
            }

            None
        }
    }
}

fn parse_hex_color(token: &str) -> Option<Color> {
    let hex = token.trim_start_matches('#');
    match hex.len() {
        3 => {
            let mut chars = hex.chars();
            let r = chars.next().and_then(parse_hex_nibble)?;
            let g = chars.next().and_then(parse_hex_nibble)?;
            let b = chars.next().and_then(parse_hex_nibble)?;
            Some(Color::Rgb(r * 17, g * 17, b * 17))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

fn parse_hex_nibble(ch: char) -> Option<u8> {
    ch.to_digit(16).map(|digit| digit as u8)
}

fn parse_border_set(lines: &HashMap<String, String>, use_double: bool) -> border::Set {
    let fallback = if use_double {
        border::DOUBLE
    } else {
        border::PLAIN
    };

    let (left_top_key, right_top_key, left_bottom_key, right_bottom_key, vert_key, horiz_key) =
        if use_double {
            (
                "dlefttop",
                "drighttop",
                "dleftbottom",
                "drightbottom",
                "dvert",
                "dhoriz",
            )
        } else {
            (
                "lefttop",
                "righttop",
                "leftbottom",
                "rightbottom",
                "vert",
                "horiz",
            )
        };

    border::Set {
        top_left: line_symbol(lines, left_top_key, "lefttop", fallback.top_left),
        top_right: line_symbol(lines, right_top_key, "righttop", fallback.top_right),
        bottom_left: line_symbol(lines, left_bottom_key, "leftbottom", fallback.bottom_left),
        bottom_right: line_symbol(
            lines,
            right_bottom_key,
            "rightbottom",
            fallback.bottom_right,
        ),
        vertical_left: line_symbol(lines, vert_key, "vert", fallback.vertical_left),
        vertical_right: line_symbol(lines, vert_key, "vert", fallback.vertical_right),
        horizontal_top: line_symbol(lines, horiz_key, "horiz", fallback.horizontal_top),
        horizontal_bottom: line_symbol(lines, horiz_key, "horiz", fallback.horizontal_bottom),
    }
}

fn line_symbol(
    lines: &HashMap<String, String>,
    preferred_key: &str,
    fallback_key: &str,
    default_symbol: &'static str,
) -> &'static str {
    if let Some(value) = lines
        .get(preferred_key)
        .or_else(|| lines.get(fallback_key))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        Box::leak(value.to_string().into_boxed_str())
    } else {
        default_symbol
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_gray_and_hex_colors() {
        assert_eq!(parse_direct_color("rgb141"), Some(Color::Indexed(77)));
        assert_eq!(parse_direct_color("gray22"), Some(Color::Indexed(254)));
        assert_eq!(parse_direct_color("#abc"), Some(Color::Rgb(170, 187, 204)));
        assert_eq!(parse_direct_color("#112233"), Some(Color::Rgb(17, 34, 51)));
    }

    #[test]
    fn resolves_aliases_in_styles() {
        let mut aliases = HashMap::new();
        aliases.insert("main".to_string(), "#123".to_string());
        aliases.insert("highlight".to_string(), "main".to_string());
        let style = parse_style_spec("highlight;default;bold", &aliases);

        assert_eq!(style.fg, Some(Color::Rgb(17, 34, 51)));
        assert_eq!(style.bg, Some(Color::Reset));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn parses_mc_default_skin() {
        let skin = UiSkin::from_ini("default", include_str!("../assets/skins/default.ini"))
            .expect("default skin should parse");

        assert_eq!(skin.name(), "default");
        assert_eq!(skin.style("core", "_default_").bg, Some(Color::Blue));
        assert_eq!(skin.style("statusbar", "_default_").bg, Some(Color::Cyan));
    }
}
