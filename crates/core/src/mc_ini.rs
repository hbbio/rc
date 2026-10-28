//! Ordered MC/GLib-style INI data shared by file associations and highlighting.
use std::collections::HashMap;

#[derive(Debug)]
pub(crate) struct Section {
    pub name: String,
    pub values: HashMap<String, String>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

pub(crate) fn parse(source: &str) -> Result<Vec<Section>, String> {
    if source.len() > 1024 * 1024 || source.contains('\0') {
        return Err("configuration exceeds 1 MiB or contains NUL".into());
    }
    let mut sections: Vec<Section> = Vec::new();
    let mut current = None;
    for (index, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        let error = |message: &str| format!("line {}: {message}", index + 1);
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if name.is_empty() || name.contains(['[', ']']) {
                return Err(error("invalid section name"));
            }
            current = Some(
                match sections.iter().position(|section| section.name == name) {
                    Some(index) => index,
                    None => {
                        if sections.len() >= 2048 {
                            return Err(error("more than 2048 sections"));
                        }
                        sections.push(Section {
                            name: name.into(),
                            values: HashMap::new(),
                        });
                        sections.len() - 1
                    }
                },
            );
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(error("expected key=value or [section]"));
        };
        let Some(section) = current else {
            return Err(error("key appears before a section"));
        };
        if key.trim().is_empty() {
            return Err(error("empty key"));
        }
        let value = unescape(value.trim()).map_err(|message| error(&message))?;
        sections[section].values.insert(key.trim().into(), value);
    }
    Ok(sections)
}

fn unescape(value: &str) -> Result<String, String> {
    let mut decoded = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        decoded.push(match chars.next() {
            Some('\\') => '\\',
            Some('s') => ' ',
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some(';') => ';',
            Some(ch) => {
                return Err(format!(
                    "invalid INI escape \\{ch}; use \\\\ for a backslash"
                ));
            }
            None => return Err("trailing INI escape".into()),
        });
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_first_section_order_and_last_key_value() {
        let sections =
            parse("[first]\nk=old\n[second]\nk=2\n[first]\nk=new\nx=\\s\\\\\\t\n").unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "first");
        assert_eq!(sections[0].get("k"), Some("new"));
        assert_eq!(sections[0].get("x"), Some(" \\\t"));
    }

    #[test]
    fn rejects_malformed_input_instead_of_silently_changing_rules() {
        for source in [
            "key=value",
            "[]\nx=1",
            "[s]\nbad",
            "[s]\nx=\\q",
            "[s]\nx=\0",
        ] {
            assert!(parse(source).is_err(), "{source:?}");
        }
    }
}
