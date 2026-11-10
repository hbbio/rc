//! RFC 3659 MLSD, parsed as bytes so filename encodings are never guessed.
use super::*;
use crate::FileEntryMetadata;
use std::collections::HashSet;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

fn invalid(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Invalid FTP MLSD listing: {message}"),
    )
}
fn number(value: &[u8]) -> io::Result<u64> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return Err(invalid("invalid numeric fact"));
    }
    std::str::from_utf8(value)
        .map_err(|_| invalid("invalid numeric fact"))?
        .parse()
        .map_err(|_| invalid("numeric fact overflow"))
}
fn time(value: &[u8]) -> io::Result<std::time::SystemTime> {
    let text = std::str::from_utf8(value).map_err(|_| invalid("invalid timestamp"))?;
    let time = chrono::NaiveDateTime::parse_from_str(text, "%Y%m%d%H%M%S%.f")
        .map_err(|_| invalid("invalid timestamp"))?;
    Ok(time.and_utc().into())
}

pub(super) fn parse(
    parent: &Path,
    bytes: &[u8],
    cancel: &Cancellation<'_>,
) -> io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    let mut names = HashSet::new();
    let mut budget = 0_usize;
    if bytes.is_empty() {
        return Ok(entries);
    }
    // curl converts FTP listing CRLF terminators to LF in ASCII transfer mode.
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(invalid("truncated listing line"));
    }
    for line in bytes[..bytes.len() - 1].split(|byte| *byte == b'\n') {
        cancel.check()?;
        if line.is_empty() {
            return Err(invalid(
                "empty listing record (CR/LF filenames are unsupported)",
            ));
        }
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.len() > 128 * 1024 {
            return Err(invalid("listing line exceeds 128 KiB"));
        }
        let separator = line
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| invalid("missing filename separator"))?;
        let (facts, name) = (&line[..separator], &line[separator + 1..]);
        if facts.last() != Some(&b';') {
            return Err(invalid("missing fact terminator"));
        }
        let mut kind = None;
        let mut size = None;
        let mut modified = None;
        let mut mode = None;
        let mut uid = None;
        let mut gid = None;
        let mut seen = HashSet::new();
        for fact in facts[..facts.len() - 1].split(|byte| *byte == b';') {
            if seen.len() >= 64 {
                return Err(invalid("more than 64 facts per entry"));
            }
            let equals = fact
                .iter()
                .position(|byte| *byte == b'=')
                .ok_or_else(|| invalid("invalid fact"))?;
            let key = fact[..equals].to_ascii_lowercase();
            let value = &fact[equals + 1..];
            if key.is_empty()
                || !key
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(byte))
                || !seen.insert(key.clone())
            {
                return Err(invalid("invalid or duplicate fact"));
            }
            match key.as_slice() {
                b"type" => kind = Some(value.to_ascii_lowercase()),
                b"size" => size = Some(number(value)?),
                b"modify" => modified = Some(time(value)?),
                b"unix.mode" => {
                    mode = Some(
                        u32::from_str_radix(
                            std::str::from_utf8(value).map_err(|_| invalid("invalid mode"))?,
                            8,
                        )
                        .map_err(|_| invalid("invalid mode"))?,
                    );
                }
                b"unix.uid" => {
                    uid = Some(u32::try_from(number(value)?).map_err(|_| invalid("invalid uid"))?)
                }
                b"unix.gid" => {
                    gid = Some(u32::try_from(number(value)?).map_err(|_| invalid("invalid gid"))?)
                }
                _ => {}
            }
        }
        let kind = kind.ok_or_else(|| {
            invalid("server omitted type facts; MLSD with type and size is required")
        })?;
        if kind == b"cdir" || kind == b"pdir" {
            continue;
        }
        validate_path(Path::new(OsStr::from_bytes(name)))?;
        if name.contains(&b'/') || name == b"." || name == b".." || name.is_empty() {
            return Err(invalid("non-child filename"));
        }
        let name = PathBuf::from(OsString::from_vec(name.to_vec()));
        if !names.insert(name.clone()) {
            return Err(invalid("duplicate filename"));
        }
        let path = parent.join(name);
        super::super::validate_relative(&path)?;
        budget = budget.saturating_add(
            (path.as_os_str().len()
                + path
                    .file_name()
                    .map_or(0, |name| name.len().saturating_mul(4))
                + 128)
                .saturating_mul(4),
        );
        if entries.len() >= 100_000 || budget > LIST_LIMIT {
            return Err(invalid("more than 100,000 entries or 16 MiB of metadata"));
        }
        let is_dir = kind == b"dir";
        let is_file = kind == b"file";
        let symlink = kind.starts_with(b"os.unix=slink") || kind.starts_with(b"os.unix=symlink");
        let file_type = if is_dir {
            0o040000
        } else if is_file {
            0o100000
        } else if symlink {
            0o120000
        } else {
            0
        };
        if mode.is_some_and(|mode| {
            mode > 0o177777 || (mode & 0o170000 != 0 && mode & 0o170000 != file_type)
        }) {
            return Err(invalid("contradictory mode and type"));
        }
        let mode = mode
            .map(|mode| file_type | (mode & 0o7777))
            .or_else(|| (!is_dir && !is_file).then_some(file_type));
        entries.push(FileEntry {
            name: rc_shell::encode_path_text(path.file_name().expect("child name")),
            path,
            kind: if is_dir {
                FileEntryKind::Directory
            } else {
                FileEntryKind::File
            },
            size: if is_dir {
                0
            } else if is_file {
                size.ok_or_else(|| invalid("server omitted regular file size"))?
            } else {
                size.unwrap_or(0)
            },
            modified,
            metadata: FileEntryMetadata {
                mode,
                user_id: uid,
                group_id: gid,
                symlink_target: symlink.then_some(false),
                ..Default::default()
            },
        });
    }
    Ok(entries)
}
