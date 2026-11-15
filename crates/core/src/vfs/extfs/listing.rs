//! The byte-oriented `list` interchange format used by MC extfs helpers.
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Utc};

use super::super::Cancellation;
use crate::{FileEntry, FileEntryKind, FileEntryMetadata};

const ENTRY_LIMIT: usize = 100_000;
const DEPTH_LIMIT: usize = 256;
const METADATA_LIMIT: usize = 64 * 1024 * 1024;

pub(super) struct Member {
    pub entry: FileEntry,
    pub extractable: bool,
}

pub(super) struct Listing {
    pub members: BTreeMap<PathBuf, Member>,
    pub children: BTreeMap<PathBuf, Vec<PathBuf>>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid extfs listing: {message}"),
    )
}

fn token<'a>(line: &'a [u8], position: &mut usize) -> io::Result<&'a [u8]> {
    while line.get(*position).is_some_and(u8::is_ascii_whitespace) {
        *position += 1;
    }
    let start = *position;
    while line
        .get(*position)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        *position += 1;
    }
    if *position == start {
        return Err(invalid("missing metadata field"));
    }
    Ok(&line[start..*position])
}

fn number<T: std::str::FromStr>(bytes: &[u8]) -> Option<T> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn mode(bytes: &[u8]) -> io::Result<u32> {
    if bytes.len() != 10 {
        return Err(invalid("permission field must have ten characters"));
    }
    let mut result = match bytes[0] {
        b'-' => 0o100000,
        b'd' => 0o040000,
        b'l' => 0o120000,
        b'b' => 0o060000,
        b'c' => 0o020000,
        b'p' => 0o010000,
        b's' => 0o140000,
        b'?' => 0, // Some helpers cannot identify an entry's storage type.
        _ => return Err(invalid("unknown file type")),
    };
    for (index, &byte) in bytes[1..].iter().enumerate() {
        let expected = b"rwx"[index % 3];
        match byte {
            b'-' => {}
            value if value == expected => result |= 1 << (8 - index),
            b's' | b'S' if index == 2 || index == 5 => {
                result |= if index == 2 { 0o4000 } else { 0o2000 };
                if byte == b's' {
                    result |= 1 << (8 - index);
                }
            }
            b't' | b'T' if index == 8 => {
                result |= 0o1000;
                if byte == b't' {
                    result |= 1;
                }
            }
            _ => return Err(invalid("invalid permissions")),
        }
    }
    Ok(result)
}

fn modified(line: &[u8], position: &mut usize) -> io::Result<SystemTime> {
    let first = token(line, position)?;
    let second = token(line, position)?;
    let ascii = |bytes| std::str::from_utf8(bytes).map_err(|_| invalid("non-ASCII date"));
    let date = if first.contains(&b'-') || first.contains(&b'/') {
        let text = format!("{} {}", ascii(first)?, ascii(second)?);
        let formats = if first.contains(&b'/') {
            match first.rsplit(|byte| *byte == b'/').next().map(<[u8]>::len) {
                Some(2) => ["%m/%d/%y %H:%M:%S", "%m/%d/%y %H:%M"],
                Some(4) => ["%m/%d/%Y %H:%M:%S", "%m/%d/%Y %H:%M"],
                _ => return Err(invalid("invalid year")),
            }
        } else if first
            .split(|byte| *byte == b'-')
            .next()
            .is_some_and(|part| part.len() == 4)
        {
            ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        } else {
            ["%m-%d-%Y %H:%M:%S", "%m-%d-%Y %H:%M"]
        };
        formats
            .into_iter()
            .find_map(|format| NaiveDateTime::parse_from_str(&text, format).ok())
    } else {
        let third = token(line, position)?;
        if third.contains(&b':') {
            let now = Utc::now();
            let text = format!(
                "{} {} {} {}",
                now.year(),
                ascii(first)?,
                ascii(second)?,
                ascii(third)?
            );
            let parsed = ["%Y %b %d %H:%M:%S", "%Y %b %d %H:%M"]
                .into_iter()
                .find_map(|format| NaiveDateTime::parse_from_str(&text, format).ok());
            // ls uses the current year, except recent dates across New Year.
            parsed.map(|date| {
                if date.and_utc() > now + chrono::Duration::days(1) {
                    date.with_year(now.year() - 1).unwrap_or(date)
                } else {
                    date
                }
            })
        } else {
            NaiveDate::parse_from_str(
                &format!("{} {} {}", ascii(first)?, ascii(second)?, ascii(third)?),
                "%b %d %Y",
            )
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
        }
    };
    date.map(|date| SystemTime::from(date.and_utc()))
        .ok_or_else(|| invalid("invalid date"))
}

fn relative(bytes: &[u8]) -> io::Result<PathBuf> {
    if bytes.contains(&0) || bytes.len() > 65_536 {
        return Err(invalid("invalid filename"));
    }
    let raw = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    let mut path = PathBuf::new();
    let mut depth = 0;
    for component in raw.components() {
        match component {
            Component::Normal(name) => {
                depth += 1;
                if depth > DEPTH_LIMIT {
                    return Err(invalid("path exceeds 256 components"));
                }
                path.push(name);
            }
            Component::CurDir => {}
            _ => return Err(invalid("filename escapes its root")),
        }
    }
    if path.as_os_str().is_empty() {
        return Err(invalid("empty or dot filename"));
    }
    Ok(path)
}

fn parse_line(line: &[u8]) -> io::Result<Member> {
    let mut position = 0;
    let mode = mode(token(line, &mut position)?)?;
    let links: u64 =
        number(token(line, &mut position)?).ok_or_else(|| invalid("invalid link count"))?;
    let user_id = number(token(line, &mut position)?);
    let group_id = number(token(line, &mut position)?);
    let size = number(token(line, &mut position)?).ok_or_else(|| invalid("invalid size"))?;
    let modified = modified(line, &mut position)?;
    if !line.get(position).is_some_and(u8::is_ascii_whitespace) {
        return Err(invalid("missing filename separator"));
    }
    // MC helpers pad this field. A name beginning with whitespace must be
    // emitted as ./<name>, which keeps the bytes unambiguous.
    while line.get(position).is_some_and(u8::is_ascii_whitespace) {
        position += 1;
    }
    let mut name = &line[position..];
    let symlink = mode & 0o170000 == 0o120000;
    let link = (symlink || links > 1)
        .then(|| name.windows(4).position(|part| part == b" -> "))
        .flatten();
    if let Some(index) = link {
        name = &name[..index];
    }
    let path = relative(name)?;
    let kind = if mode & 0o170000 == 0o040000 {
        FileEntryKind::Directory
    } else {
        FileEntryKind::File
    };
    Ok(Member {
        extractable: mode & 0o170000 == 0o100000 && link.is_none(),
        entry: FileEntry {
            name: rc_shell::encode_path_text(path.file_name().unwrap()),
            path,
            kind,
            size,
            modified: Some(modified),
            metadata: FileEntryMetadata {
                mode: Some(mode),
                hard_links: Some(links),
                user_id,
                group_id,
                symlink_target: symlink.then_some(false),
                ..FileEntryMetadata::default()
            },
        },
    })
}

fn directory(path: PathBuf) -> Member {
    Member {
        extractable: false,
        entry: FileEntry {
            name: path
                .file_name()
                .map_or_else(String::new, rc_shell::encode_path_text),
            path,
            kind: FileEntryKind::Directory,
            size: 0,
            modified: None,
            metadata: FileEntryMetadata::default(),
        },
    }
}

impl Listing {
    pub fn parse(bytes: &[u8], cancel: &Cancellation<'_>) -> io::Result<Self> {
        let mut members = BTreeMap::new();
        let mut order = Vec::new();
        let mut explicit = HashSet::new();
        let mut metadata_bytes = 0_usize;
        let mut reserve = |path: &Path| -> io::Result<()> {
            // Includes path copies in membership/order/children maps and their
            // allocation overhead. Check before synthesizing each ancestor.
            metadata_bytes =
                metadata_bytes.saturating_add((path.as_os_str().len() + 128).saturating_mul(8));
            if metadata_bytes > METADATA_LIMIT {
                return Err(invalid("metadata exceeds 64 MiB"));
            }
            Ok(())
        };
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            cancel.check()?;
            let member = parse_line(line)?;
            let path = member.entry.path.clone();
            if !explicit.insert(path.clone()) {
                return Err(invalid("duplicate filename"));
            }
            let mut parents: Vec<_> = path.ancestors().skip(1).collect();
            parents.reverse();
            for parent in parents {
                cancel.check()?;
                if let Some(existing) = members.get(parent) {
                    let existing: &Member = existing;
                    if !existing.entry.is_dir() {
                        return Err(invalid("file used as a directory"));
                    }
                } else {
                    reserve(parent)?;
                    order.push(parent.to_path_buf());
                    members.insert(parent.to_path_buf(), directory(parent.to_path_buf()));
                }
            }
            if let Some(existing) = members.get(&path) {
                if !existing.entry.is_dir() || !member.entry.is_dir() {
                    return Err(invalid("file used as a directory"));
                }
            } else {
                reserve(&path)?;
                order.push(path.clone());
            }
            members.insert(path, member);
            if members.len() > ENTRY_LIMIT {
                return Err(invalid("more than 100,000 entries"));
            }
        }
        members
            .entry(PathBuf::new())
            .or_insert_with(|| directory(PathBuf::new()));
        let mut children: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        for path in &order {
            cancel.check()?;
            if let Some(parent) = path.parent() {
                children
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(path.clone());
            }
        }
        let files: Vec<_> = members
            .values()
            .filter(|member| !member.entry.is_dir())
            .map(|member| {
                (
                    member.entry.path.clone(),
                    member.entry.size,
                    member.entry.modified,
                )
            })
            .collect();
        for member in members.values_mut().filter(|member| member.entry.is_dir()) {
            member.entry.size = 0;
        }
        for (path, size, modified) in files {
            for parent in path.ancestors().skip(1) {
                cancel.check()?;
                let entry = &mut members.get_mut(parent).expect("synthesized ancestor").entry;
                entry.size = entry.size.saturating_add(size);
                entry.modified = entry.modified.max(modified);
            }
        }
        Ok(Self { members, children })
    }

    pub fn member(&self, path: &Path) -> io::Result<&Member> {
        self.members
            .get(path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "extfs member not found"))
    }
}
