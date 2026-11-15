//! Ordered MC file-association rules, independent of process execution.
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use regex::bytes::{Regex, RegexBuilder};

use crate::{
    mc_ini::{self, Section},
    settings_io::parse_bool,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExtensionAction {
    Open,
    View,
    Edit,
}

impl ExtensionAction {
    const fn index(self) -> usize {
        match self {
            Self::Open => 0,
            Self::View => 1,
            Self::Edit => 2,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Actions([Option<String>; 3]);

#[derive(Clone, Debug)]
enum NameMatcher {
    Regex(Regex),
    Shell { pattern: Vec<u8>, ignore_case: bool },
}

impl NameMatcher {
    fn matches(&self, name: &[u8]) -> bool {
        match self {
            Self::Regex(regex) => regex.is_match(name),
            Self::Shell {
                pattern,
                ignore_case,
            } => {
                let name = if pattern.starts_with(b".") {
                    match name.len().checked_sub(pattern.len()) {
                        Some(start) => &name[start..],
                        None => return false,
                    }
                } else {
                    name
                };
                if *ignore_case {
                    name.eq_ignore_ascii_case(pattern)
                } else {
                    name == pattern
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Matcher {
    Directory(Regex),
    File {
        name: Option<NameMatcher>,
        file_type: Option<Regex>,
    },
}

#[derive(Clone, Debug)]
struct Rule {
    matcher: Matcher,
    actions: Arc<Actions>,
}

/// Rules preserve section order. Includes supply the entire action set; an absent
/// action falls back to [Default], while an explicit empty action suppresses it.
#[derive(Clone, Debug)]
pub struct ExtensionRules {
    rules: Vec<Rule>,
    default: Arc<Actions>,
}

impl ExtensionRules {
    pub fn has_directory_match(&self, path: &Path) -> bool {
        self.rules.iter().any(|rule| matches!(&rule.matcher, Matcher::Directory(regex) if regex.is_match(path.as_os_str().as_encoded_bytes())))
    }
    pub fn parse(source: &str) -> Result<Self, String> {
        let sections = mc_ini::parse(source)?;
        let version = sections
            .iter()
            .find(|section| section.name == "mc.ext.ini")
            .and_then(|section| section.get("Version"));
        if version != Some("4.0") {
            return Err("mc.ext.ini requires [mc.ext.ini] with Version=4.0".into());
        }
        let indexes = sections
            .iter()
            .enumerate()
            .map(|(index, section)| (section.name.as_str(), index))
            .collect();
        let mut actions_cache = vec![None; sections.len()];
        let default = sections
            .iter()
            .position(|section| section.name == "Default")
            .map(|index| resolve_actions(&sections, &indexes, &mut actions_cache, index))
            .transpose()?
            .unwrap_or_default();
        let mut rules = Vec::new();
        for (index, section) in sections.iter().enumerate() {
            if matches!(section.name.as_str(), "mc.ext.ini" | "Default") {
                continue;
            }
            // Validate even unused includes, so configuration mistakes do not
            // suddenly surface only when a particular filename is selected.
            let actions = resolve_actions(&sections, &indexes, &mut actions_cache, index)?;
            if section.name.starts_with("Include/") {
                continue;
            }
            let matcher = if let Some(pattern) = section.get("Directory") {
                Matcher::Directory(compile_pattern(section, pattern, false)?)
            } else {
                let name = if let Some(pattern) = section.get("Regex") {
                    Some(NameMatcher::Regex(compile_pattern(
                        section,
                        pattern,
                        boolean(section, "RegexIgnoreCase")?,
                    )?))
                } else {
                    section
                        .get("Shell")
                        .map(|pattern| {
                            Ok::<_, String>(NameMatcher::Shell {
                                pattern: pattern.as_bytes().to_vec(),
                                ignore_case: boolean(section, "ShellIgnoreCase")?,
                            })
                        })
                        .transpose()?
                };
                let file_type = section
                    .get("Type")
                    .map(|pattern| {
                        compile_pattern(section, pattern, boolean(section, "TypeIgnoreCase")?)
                    })
                    .transpose()?;
                Matcher::File { name, file_type }
            };
            rules.push(Rule { matcher, actions });
        }
        Ok(Self { rules, default })
    }

    /// The caller can obtain `file -b` output on a worker only when needed.
    pub fn needs_file_type(&self, path: &Path, is_directory: bool) -> bool {
        let name = basename_bytes(path);
        for rule in &self.rules {
            match &rule.matcher {
                Matcher::Directory(regex)
                    if is_directory && regex.is_match(path.as_os_str().as_encoded_bytes()) =>
                {
                    return false;
                }
                Matcher::File {
                    name: matcher,
                    file_type,
                } if matcher.as_ref().is_none_or(|matcher| matcher.matches(name)) => {
                    if file_type.is_some() {
                        return true;
                    }
                    if matcher.is_some() {
                        return false;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// `file_type=None` means the file utility is unavailable: name-based rules
    /// still work, and type-only sections do not become catch-all rules.
    pub fn resolve(
        &self,
        path: &Path,
        is_directory: bool,
        file_type: Option<&[u8]>,
        action: ExtensionAction,
    ) -> Option<&str> {
        let name = basename_bytes(path);
        let selected = self.rules.iter().find(|rule| match &rule.matcher {
            Matcher::Directory(regex) => {
                is_directory && regex.is_match(path.as_os_str().as_encoded_bytes())
            }
            Matcher::File {
                name: matcher,
                file_type: kind,
            } => {
                if matcher
                    .as_ref()
                    .is_some_and(|matcher| !matcher.matches(name))
                {
                    return false;
                }
                match (kind, file_type) {
                    (Some(regex), Some(output)) => regex.is_match(output),
                    _ => matcher.is_some(),
                }
            }
        });
        selected
            .and_then(|rule| rule.actions.0[action.index()].as_deref())
            .or(self.default.0[action.index()].as_deref())
            .filter(|action| !action.is_empty())
    }
}

fn basename_bytes(path: &Path) -> &[u8] {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .as_encoded_bytes()
}

fn compile_pattern(section: &Section, pattern: &str, ignore_case: bool) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|error| format!("[{}]: {error}", section.name))
}

fn boolean(section: &Section, key: &str) -> Result<bool, String> {
    section
        .get(key)
        .map(|value| {
            parse_bool(value).ok_or_else(|| format!("[{}]: invalid {key} boolean", section.name))
        })
        .unwrap_or(Ok(false))
}

fn resolve_actions(
    sections: &[Section],
    indexes: &HashMap<&str, usize>,
    cache: &mut [Option<Arc<Actions>>],
    start: usize,
) -> Result<Arc<Actions>, String> {
    let mut index = start;
    let mut visited = HashSet::new();
    let actions = loop {
        if let Some(actions) = &cache[index] {
            break Arc::clone(actions);
        }
        let section = &sections[index];
        if !visited.insert(index) {
            return Err(format!(
                "[{}]: cyclic Include reference",
                sections[start].name
            ));
        }
        let Some(include) = section.get("Include") else {
            break Arc::new(Actions(
                ["Open", "View", "Edit"].map(|key| section.get(key).map(str::to_owned)),
            ));
        };
        let name = format!("Include/{include}");
        index = indexes
            .get(name.as_str())
            .copied()
            .ok_or_else(|| format!("[{}]: missing [{name}]", section.name))?;
    };
    for index in visited {
        cache[index] = Some(Arc::clone(&actions));
    }
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ExtensionAction::{Edit, Open, View};

    fn rules(body: &str) -> ExtensionRules {
        ExtensionRules::parse(&format!("[mc.ext.ini]\nVersion=4.0\n{body}")).unwrap()
    }

    #[test]
    fn many_includes_share_large_actions_and_deep_chains_are_iterative() {
        let mut source = String::from("[mc.ext.ini]\nVersion=4.0\n");
        for index in 0..1000 {
            source.push_str(&format!("[rule{index}]\nShell=.x{index}\nInclude=0\n"));
        }
        for index in 0..999 {
            source.push_str(&format!("[Include/{index}]\nInclude={}\n", index + 1));
        }
        source.push_str("[Include/999]\nOpen=");
        source.push_str(&"x".repeat(500_000));
        let rules = ExtensionRules::parse(&source).unwrap();
        assert_eq!(rules.rules.len(), 1000);
        assert!(
            rules
                .rules
                .iter()
                .all(|rule| Arc::ptr_eq(&rule.actions, &rules.rules[0].actions))
        );
        assert_eq!(
            rules
                .resolve(Path::new("file.x999"), false, None, Open)
                .unwrap()
                .len(),
            500_000
        );
    }

    #[test]
    fn first_match_wins_with_default_for_missing_but_not_empty_actions() {
        let rules = rules(
            "[one]\nShell=.rs\nView=\n[two]\nShell=.rs\nOpen=second\n[Default]\nOpen=default\nView=default-view\nEdit=default-edit",
        );
        assert_eq!(
            rules.resolve(Path::new("file.rs"), false, None, Open),
            Some("default")
        );
        assert_eq!(rules.resolve(Path::new("file.rs"), false, None, View), None);
        assert_eq!(
            rules.resolve(Path::new("file.rs"), false, None, Edit),
            Some("default-edit")
        );
        assert_eq!(
            rules.resolve(Path::new("other"), false, None, View),
            Some("default-view")
        );
    }

    #[test]
    fn includes_can_appear_later_and_replace_local_actions() {
        let rules = rules(
            "[source]\nShell=.rs\nOpen=ignored\nInclude=one\n[Include/two]\nOpen=shared\n[Include/one]\nInclude=two\nView=ignored\n[Default]\nView=fallback",
        );
        assert_eq!(
            rules.resolve(Path::new("file.rs"), false, None, Open),
            Some("shared")
        );
        assert_eq!(
            rules.resolve(Path::new("file.rs"), false, None, View),
            Some("fallback")
        );
    }

    #[test]
    fn repeated_sections_merge_at_original_position_and_last_key_wins() {
        let rules = rules(
            "[first]\nShell=.txt\nOpen=old\n[second]\nShell=.rs\nOpen=second\n[first]\nShell=.rs\nOpen=last",
        );
        assert_eq!(
            rules.resolve(Path::new("file.rs"), false, None, Open),
            Some("last")
        );
    }

    #[test]
    fn directory_overrides_other_matchers_and_matches_full_path() {
        let rules = rules(
            "[dir]\nDirectory=/project/[^/]+$\nRegex=[\nShell=.never\nType=[\nOpen=directory\n[Default]\nOpen=fallback",
        );
        assert_eq!(
            rules.resolve(Path::new("/project/src"), true, None, Open),
            Some("directory")
        );
        assert_eq!(
            rules.resolve(Path::new("/project/src"), false, None, Open),
            Some("fallback")
        );
        assert!(!rules.needs_file_type(Path::new("/project/src"), true));
    }

    #[test]
    fn regex_takes_precedence_over_shell_and_both_combine_with_type() {
        let rules = rules(
            r"[source]
Regex=\\.rs$
RegexIgnoreCase=true
Shell=.ignored
Type=^text
TypeIgnoreCase=true
Open=source
[typeonly]
Type=^image
Open=image
[Default]
Open=fallback",
        );
        assert!(rules.needs_file_type(Path::new("main.RS"), false));
        assert_eq!(
            rules.resolve(Path::new("main.RS"), false, Some(b"TEXT source"), Open),
            Some("source")
        );
        assert_eq!(
            rules.resolve(Path::new("main.rs"), false, Some(b"binary"), Open),
            Some("fallback")
        );
        assert_eq!(
            rules.resolve(Path::new("main.rs"), false, None, Open),
            Some("source")
        );
        assert_eq!(
            rules.resolve(Path::new("other"), false, None, Open),
            Some("fallback")
        );
        assert_eq!(
            rules.resolve(Path::new("other"), false, Some(b"image PNG"), Open),
            Some("image")
        );
    }

    #[test]
    fn shell_matches_exact_basename_or_literal_suffix_without_globbing() {
        let rules = rules(
            "[make]\nShell=Makefile\nOpen=make\n[tar]\nShell=.tar.gz\nShellIgnoreCase=true\nOpen=tar\n[glob]\nShell=*.foo\nOpen=literal",
        );
        for (path, expected) in [
            ("/tmp/Makefile", Some("make")),
            ("makefile", None),
            ("PrefixMakefile", None),
            ("A.TAR.GZ", Some("tar")),
            ("a.foo", None),
            ("*.foo", Some("literal")),
        ] {
            assert_eq!(rules.resolve(Path::new(path), false, None, Open), expected);
        }
        assert!(!rules.needs_file_type(Path::new("Makefile"), false));
    }

    #[cfg(unix)]
    #[test]
    fn filename_matching_preserves_non_utf8_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let rules = rules(
            r"[raw]
Regex=(?-u:\\xFF)\\.dat$
Open=raw",
        );
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/name\xff.dat"));
        assert_eq!(rules.resolve(path, false, None, Open), Some("raw"));
    }

    #[test]
    fn rejects_missing_versions_bad_patterns_and_broken_or_cyclic_includes() {
        for body in [
            "[x]\nRegex=[",
            "[x]\nInclude=missing",
            "[Include/a]\nInclude=b\n[Include/b]\nInclude=a",
            "[x]\nShell=a\nShellIgnoreCase=perhaps",
        ] {
            assert!(ExtensionRules::parse(&format!("[mc.ext.ini]\nVersion=4.0\n{body}")).is_err());
        }
        assert!(ExtensionRules::parse("[x]\nShell=a").is_err());
        assert!(ExtensionRules::parse("[mc.ext.ini]\nVersion=9").is_err());
    }
}
