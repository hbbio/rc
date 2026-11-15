use std::sync::{Arc, OnceLock};

use regex::Regex;

use crate::{AppState, ConfigurationFile, FileEntry, PanelState, mc_ini, settings_io::parse_bool};

#[derive(Debug)]
pub(crate) struct FileHighlightRules {
    source: String,
    rules: Vec<Rule>,
}

impl PartialEq for FileHighlightRules {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}
impl Eq for FileHighlightRules {}

#[derive(Debug)]
struct Rule {
    group: String,
    matcher: Matcher,
}

#[derive(Debug)]
enum Matcher {
    Type(FileType),
    Regex(Regex),
    Extensions {
        values: Vec<String>,
        case_sensitive: bool,
    },
}

#[derive(Debug)]
enum FileType {
    File,
    Executable,
    Directory,
    LinkDirectory,
    Link,
    Hardlink,
    Symlink,
    StaleLink,
    Device,
    Block,
    Character,
    Special,
    Socket,
    Fifo,
    Door,
}

impl FileType {
    fn parse(value: &str) -> Result<Self, String> {
        Ok(match value {
            "FILE" => Self::File,
            "FILE_EXE" => Self::Executable,
            "DIR" => Self::Directory,
            "LINK_DIR" => Self::LinkDirectory,
            "LINK" => Self::Link,
            "HARDLINK" => Self::Hardlink,
            "SYMLINK" => Self::Symlink,
            "STALE_LINK" => Self::StaleLink,
            "DEVICE" => Self::Device,
            "DEVICE_BLOCK" => Self::Block,
            "DEVICE_CHAR" => Self::Character,
            "SPECIAL" => Self::Special,
            "SPECIAL_SOCKET" => Self::Socket,
            "SPECIAL_FIFO" => Self::Fifo,
            "SPECIAL_DOOR" => Self::Door,
            _ => return Err(format!("unknown file type {value}")),
        })
    }

    fn matches(&self, entry: &FileEntry) -> bool {
        let mode = entry.metadata.mode.unwrap_or(0);
        let kind = mode & 0o170000;
        let symlink = entry.metadata.symlink_target;
        let regular = !entry.is_dir() && symlink.is_none() && (kind == 0 || kind == 0o100000);
        let hardlink = regular && entry.metadata.hard_links.is_some_and(|count| count > 1);
        match self {
            Self::File => regular,
            Self::Executable => regular && mode & 0o111 != 0,
            Self::Directory => entry.is_dir(),
            Self::LinkDirectory => symlink == Some(true) && entry.is_dir(),
            Self::Link => symlink == Some(true) || hardlink,
            Self::Hardlink => hardlink,
            Self::Symlink => symlink == Some(true),
            Self::StaleLink => symlink == Some(false),
            Self::Device => symlink.is_none() && matches!(kind, 0o020000 | 0o060000),
            Self::Block => symlink.is_none() && kind == 0o060000,
            Self::Character => symlink.is_none() && kind == 0o020000,
            Self::Special => symlink.is_none() && matches!(kind, 0o010000 | 0o140000 | 0o150000),
            Self::Socket => symlink.is_none() && kind == 0o140000,
            Self::Fifo => symlink.is_none() && kind == 0o010000,
            Self::Door => symlink.is_none() && kind == 0o150000,
        }
    }
}

impl FileHighlightRules {
    fn parse(source: &str) -> Result<Self, String> {
        let mut rules = Vec::new();
        for section in mc_ini::parse(source)? {
            let error = |message: String| format!("[{}]: {message}", section.name);
            let matcher = if let Some(kind) = section.get("type") {
                Matcher::Type(FileType::parse(kind).map_err(error)?)
            } else if let Some(pattern) = section.get("regexp") {
                Matcher::Regex(Regex::new(pattern).map_err(|e| error(e.to_string()))?)
            } else if let Some(extensions) = section.get("extensions") {
                let case_sensitive = match section.get("extensions_case") {
                    None => false,
                    Some(value) => parse_bool(value)
                        .ok_or_else(|| error("invalid extensions_case boolean".into()))?,
                };
                let values = extensions
                    .split(';')
                    .filter(|value| !value.is_empty())
                    .map(|value| {
                        if case_sensitive {
                            value.to_owned()
                        } else {
                            value.to_lowercase()
                        }
                    })
                    .collect();
                Matcher::Extensions {
                    values,
                    case_sensitive,
                }
            } else {
                continue;
            };
            rules.push(Rule {
                group: section.name,
                matcher,
            });
        }
        Ok(Self {
            source: source.into(),
            rules,
        })
    }

    fn group<'a>(&'a self, entry: &FileEntry) -> Option<&'a str> {
        self.rules
            .iter()
            .find(|rule| match &rule.matcher {
                Matcher::Type(kind) => kind.matches(entry),
                Matcher::Regex(regex) => regex.is_match(&entry.name),
                Matcher::Extensions {
                    values,
                    case_sensitive,
                } => {
                    let lower;
                    let name = if *case_sensitive {
                        entry.name.as_str()
                    } else {
                        lower = entry.name.to_lowercase();
                        &lower
                    };
                    values.iter().any(|extension| {
                        name.strip_suffix(extension)
                            .is_some_and(|prefix| prefix.ends_with('.'))
                    })
                }
            })
            .map(|rule| rule.group.as_str())
    }
}

pub(crate) fn default_rules() -> Arc<FileHighlightRules> {
    static DEFAULT: OnceLock<Arc<FileHighlightRules>> = OnceLock::new();
    Arc::clone(DEFAULT.get_or_init(|| {
        Arc::new(
            FileHighlightRules::parse(ConfigurationFile::Highlighting.bundled())
                .expect("bundled highlight rules are valid"),
        )
    }))
}

impl PanelState {
    /// Matches cached listing metadata and decoded labels without filesystem IO.
    pub fn file_highlight_group(&self, entry: &FileEntry) -> Option<&str> {
        self.file_highlights.group(entry)
    }
}

impl AppState {
    /// Reload after an external editor closes. Invalid edits retain the last valid rules.
    pub fn reload_file_highlights(&mut self) {
        let result = self
            .configuration_files
            .read(ConfigurationFile::Highlighting, None)
            .map_err(|error| error.to_string())
            .and_then(|source| FileHighlightRules::parse(&source));
        match result {
            Ok(rules) => {
                let rules = Arc::new(rules);
                for panel in &mut self.panels {
                    panel.file_highlights = Arc::clone(&rules);
                }
            }
            Err(error) => self.set_status(format!("Cannot load filehighlight.ini: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConfigurationFiles, FileEntryMetadata};
    use std::fs;

    fn entry(name: &str) -> FileEntry {
        FileEntry::file(name.into(), name.into(), 0, None)
    }

    #[test]
    fn highlight_rules_use_order_precedence_and_extension_case() {
        let rules = FileHighlightRules::parse(
            r"
[preferred]
regexp=^README
extensions=rs
[source]
extensions=rs;tar.gz
[case]
extensions=TXT
extensions_case=true
[directory]
type=DIR
regexp=.*",
        )
        .unwrap();
        for (name, expected) in [
            ("README.rs", Some("preferred")),
            ("main.RS", Some("source")),
            ("a.tar.gz", Some("source")),
            ("no-rs", None),
            ("a.TXT", Some("case")),
            ("a.txt", None),
            ("other", None),
        ] {
            assert_eq!(rules.group(&entry(name)), expected, "{name}");
        }
        assert_eq!(
            rules.group(&FileEntry::parent("..".into())),
            Some("directory")
        );
    }

    #[test]
    fn highlight_regex_obeys_ini_backslash_escaping() {
        let rules = FileHighlightRules::parse(
            r"[source]
regexp=\\.rs$",
        )
        .unwrap();
        assert_eq!(rules.group(&entry("a.rs")), Some("source"));
        assert_eq!(rules.group(&entry("ars")), None);
        for source in [
            "[x]\ntype=UNKNOWN",
            "[x]\nregexp=[",
            "[x]\nextensions=x\nextensions_case=maybe",
        ] {
            assert!(FileHighlightRules::parse(source).is_err());
        }
    }

    #[test]
    fn highlights_cached_special_file_metadata_without_stat() {
        let rules = FileHighlightRules::parse(
            r"
[stale]
type=STALE_LINK
[linkdir]
type=LINK_DIR
[link]
type=SYMLINK
[exe]
type=FILE_EXE
[device]
type=DEVICE
[special]
type=SPECIAL
[hardlink]
type=HARDLINK",
        )
        .unwrap();
        let mut entry = entry("nonexistent");
        for (mode, expected) in [
            (0o100755, "exe"),
            (0o020600, "device"),
            (0o060600, "device"),
            (0o140600, "special"),
            (0o010600, "special"),
        ] {
            entry.metadata.mode = Some(mode);
            assert_eq!(rules.group(&entry), Some(expected));
        }
        entry.metadata = FileEntryMetadata {
            mode: Some(0o100644),
            hard_links: Some(2),
            ..Default::default()
        };
        assert_eq!(rules.group(&entry), Some("hardlink"));
        entry.metadata.symlink_target = Some(true);
        assert_eq!(rules.group(&entry), Some("link"));
        entry.kind = crate::FileEntryKind::Directory;
        assert_eq!(rules.group(&entry), Some("linkdir"));
        entry.metadata.symlink_target = Some(false);
        assert_eq!(rules.group(&entry), Some("stale"));
    }

    #[cfg(unix)]
    #[test]
    fn local_and_panelized_listings_retain_symlink_identity_and_refresh_targets() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("file"), "data").unwrap();
        fs::create_dir(root.path().join("dir")).unwrap();
        symlink("file", root.path().join("link")).unwrap();
        symlink("dir", root.path().join("linkdir")).unwrap();
        symlink("absent", root.path().join("stale")).unwrap();
        let mut app = crate::tests::app_with_loaded_panels(root.path().to_path_buf());
        let assert_links = |panel: &PanelState| {
            for (name, expected) in [
                ("link", Some(true)),
                ("linkdir", Some(true)),
                ("stale", Some(false)),
                ("file", None),
            ] {
                assert_eq!(
                    panel
                        .entries
                        .iter()
                        .find(|entry| entry.name == name)
                        .unwrap()
                        .metadata
                        .symlink_target,
                    expected
                );
            }
        };
        assert_links(app.active_panel());
        app.panels[0].refresh().unwrap();
        assert_links(app.active_panel());
        let entries = crate::panel::read_panelized_entries_with_cancel(
            root.path(),
            "printf '%s\\n' file link linkdir stale",
            Default::default(),
            None,
        )
        .unwrap();
        app.panels[0].entries = entries;
        assert_links(app.active_panel());
        fs::remove_file(root.path().join("file")).unwrap();
        app.panels[0].refresh().unwrap();
        let link = app
            .active_panel()
            .entries
            .iter()
            .find(|entry| entry.name == "link")
            .unwrap();
        assert_eq!(link.metadata.symlink_target, Some(false));
        assert_eq!(
            app.active_panel().file_highlight_group(link),
            Some("stalelink")
        );
    }

    #[test]
    fn reload_applies_to_both_panels_and_preserves_valid_rules_on_error() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("filehighlight.ini");
        fs::write(&path, "[one]\nextensions=rs").unwrap();
        let mut app = AppState::new(root.path().to_path_buf()).unwrap();
        app.set_configuration_files(ConfigurationFiles {
            user_directory: Some(root.path().into()),
            system_directories: vec![],
        });
        let entry = entry("main.rs");
        assert!(
            app.panels
                .iter()
                .all(|p| p.file_highlight_group(&entry) == Some("one"))
        );
        fs::write(&path, "[two]\nextensions=rs").unwrap();
        app.reload_file_highlights();
        assert!(
            app.panels
                .iter()
                .all(|p| p.file_highlight_group(&entry) == Some("two"))
        );
        fs::write(&path, "[bad]\nregexp=[").unwrap();
        app.reload_file_highlights();
        assert!(
            app.panels
                .iter()
                .all(|p| p.file_highlight_group(&entry) == Some("two"))
        );
        assert!(app.status_line.contains("Cannot load filehighlight.ini"));
    }
}
