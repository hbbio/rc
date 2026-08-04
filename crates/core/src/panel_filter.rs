use std::fmt;
use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use regex::{Regex, RegexBuilder};

use crate::{FileEntry, FindNameMode};

pub const MAX_PANEL_FILTER_CHARS: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelFilter {
    pub pattern: String,
    pub files_only: bool,
    pub name_mode: FindNameMode,
    pub case_sensitive: bool,
}

impl Default for PanelFilter {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            files_only: true,
            name_mode: FindNameMode::Glob,
            case_sensitive: true,
        }
    }
}

impl PanelFilter {
    pub fn is_active(&self) -> bool {
        !self.pattern.is_empty()
    }

    pub fn display_pattern(&self) -> &str {
        if self.is_active() {
            self.pattern.as_str()
        } else {
            "<disabled>"
        }
    }

    pub fn validate(&self) -> Result<(), PanelFilterError> {
        self.compile().map(|_| ())
    }

    pub(crate) fn compile(&self) -> Result<CompiledPanelFilter, PanelFilterError> {
        if self.pattern.chars().count() > MAX_PANEL_FILTER_CHARS {
            return Err(PanelFilterError::TooLong {
                maximum: MAX_PANEL_FILTER_CHARS,
            });
        }
        if self.pattern.chars().any(char::is_control) {
            return Err(PanelFilterError::ControlCharacter);
        }
        if self.pattern.is_empty() {
            return Ok(CompiledPanelFilter {
                matcher: PanelNameMatcher::Any,
                files_only: self.files_only,
            });
        }

        let matcher = match self.name_mode {
            FindNameMode::Glob => GlobBuilder::new(&self.pattern)
                .case_insensitive(!self.case_sensitive)
                .literal_separator(false)
                .backslash_escape(true)
                .build()
                .map(|glob| PanelNameMatcher::Glob(glob.compile_matcher()))
                .map_err(|error| PanelFilterError::InvalidPattern(error.to_string()))?,
            FindNameMode::Regex => RegexBuilder::new(&self.pattern)
                .case_insensitive(!self.case_sensitive)
                .build()
                .map(PanelNameMatcher::Regex)
                .map_err(|error| PanelFilterError::InvalidPattern(error.to_string()))?,
        };

        Ok(CompiledPanelFilter {
            matcher,
            files_only: self.files_only,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PanelFilterError {
    TooLong { maximum: usize },
    ControlCharacter,
    InvalidPattern(String),
}

impl fmt::Display for PanelFilterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { maximum } => {
                write!(formatter, "filter pattern exceeds {maximum} characters")
            }
            Self::ControlCharacter => {
                formatter.write_str("filter pattern contains a control character")
            }
            Self::InvalidPattern(message) => write!(formatter, "invalid filter pattern: {message}"),
        }
    }
}

impl std::error::Error for PanelFilterError {}

pub(crate) struct CompiledPanelFilter {
    matcher: PanelNameMatcher,
    files_only: bool,
}

impl CompiledPanelFilter {
    pub(crate) fn matches(&self, entry: &FileEntry) -> bool {
        if entry.is_parent() || (self.files_only && entry.is_dir()) {
            return true;
        }
        match &self.matcher {
            PanelNameMatcher::Any => true,
            PanelNameMatcher::Glob(matcher) => matcher.is_match(Path::new(&entry.name)),
            PanelNameMatcher::Regex(regex) => regex.is_match(&entry.name),
        }
    }
}

pub(crate) fn apply_panel_filter(
    entries: Vec<FileEntry>,
    filter: &PanelFilter,
) -> Result<Vec<FileEntry>, PanelFilterError> {
    let matcher = filter.compile()?;
    Ok(entries
        .into_iter()
        .filter(|entry| matcher.matches(entry))
        .collect())
}

enum PanelNameMatcher {
    Any,
    Glob(GlobMatcher),
    Regex(Regex),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileEntryKind, FileEntryMetadata};
    use std::path::PathBuf;

    fn entry(name: &str, kind: FileEntryKind) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind,
            size: 0,
            modified: None,
            metadata: FileEntryMetadata::default(),
        }
    }

    #[test]
    fn shell_patterns_respect_case_and_keep_directories_in_files_only_mode() {
        let filter = PanelFilter {
            pattern: String::from("*.rs"),
            ..PanelFilter::default()
        };
        let matcher = filter.compile().expect("glob should compile");

        assert!(matcher.matches(&entry("main.rs", FileEntryKind::File)));
        assert!(!matcher.matches(&entry("main.RS", FileEntryKind::File)));
        assert!(matcher.matches(&entry("build", FileEntryKind::Directory)));
        assert!(matcher.matches(&entry("..", FileEntryKind::Parent)));
    }

    #[test]
    fn regex_can_filter_directories_case_insensitively() {
        let filter = PanelFilter {
            pattern: String::from("^src$"),
            files_only: false,
            name_mode: FindNameMode::Regex,
            case_sensitive: false,
        };
        let matcher = filter.compile().expect("regex should compile");

        assert!(matcher.matches(&entry("SRC", FileEntryKind::Directory)));
        assert!(!matcher.matches(&entry("tests", FileEntryKind::Directory)));
        assert!(matcher.matches(&entry("..", FileEntryKind::Parent)));
    }

    #[test]
    fn invalid_and_oversized_patterns_are_rejected() {
        let invalid = PanelFilter {
            pattern: String::from("["),
            ..PanelFilter::default()
        };
        assert!(matches!(
            invalid.validate(),
            Err(PanelFilterError::InvalidPattern(_))
        ));

        let oversized = PanelFilter {
            pattern: "x".repeat(MAX_PANEL_FILTER_CHARS + 1),
            ..PanelFilter::default()
        };
        assert_eq!(
            oversized.validate(),
            Err(PanelFilterError::TooLong {
                maximum: MAX_PANEL_FILTER_CHARS
            })
        );
    }
}
