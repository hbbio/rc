use super::*;
use crate::{ArchiveLocation, SortMode};
use chrono::Datelike;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;

fn cancellation() -> Cancellation<'static> {
    Cancellation {
        requested: None,
        closed: None,
    }
}

fn helper(directory: &Path, name: &str, script: &str) -> Helper {
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{script}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    Helper::from_path(&path).unwrap()
}

fn fixture(directory: &Path) -> Helper {
    helper(
        directory,
        "fake",
        r#"
case "$1" in
list) printf '%s 1 42 group %s 01-02-2025 03:04 folder/file with spaces\n' '-rw-r--r--' "$(wc -c < "$2" | tr -d ' ')";;
copyout) test "$3" = 'folder/file with spaces'; cat "$2" > "$4";;
*) exit 2;;
esac
"#,
    )
}

fn member(root: &VfsPath) -> VfsPath {
    root.resolve_identity(root.identity().join("folder/file with spaces"))
        .unwrap()
}

fn read(path: &VfsPath) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    path.with_file(None, &mut |_, reader| {
        reader.read_to_end(&mut bytes)?;
        Ok(())
    })?;
    Ok(bytes)
}

#[test]
fn listing_preserves_names_dates_modes_and_synthesized_directories() {
    let bytes = b"-rwsr-Sr-t 1 42 group 3 Jan 2 2025   top/file -> literal\n\
        drwxr-xr-x 2 root root 4096 01-02-2025 03:04 top/\n\
        lrwxrwxrwx 1 root root 0 01/02/25 03:04:05 link -> top/file\n\
        -rw-r--r-- 2 1 2 3 2025-01-02 03:04 hard -> top/file\n\
        -rw-r--r-- 1 1 2 4 Jan 2 03:04:05 ./ leading space \xff\n";
    let listing = Listing::parse(bytes, &cancellation()).unwrap();
    assert_eq!(listing.member(Path::new("top")).unwrap().entry.size, 3);
    let file = listing.member(Path::new("top/file -> literal")).unwrap();
    assert!(file.extractable);
    assert_eq!(file.entry.metadata.mode, Some(0o107745));
    assert_eq!(file.entry.metadata.user_id, Some(42));
    assert_eq!(file.entry.metadata.group_id, None);
    let link = listing.member(Path::new("link")).unwrap();
    assert!(!link.extractable);
    assert_eq!(
        chrono::DateTime::<chrono::Utc>::from(link.entry.modified.unwrap()).year(),
        2025
    );
    assert!(!listing.member(Path::new("hard")).unwrap().extractable);
    assert!(
        listing
            .members
            .keys()
            .any(|path| path.as_os_str().as_bytes() == b" leading space \xff")
    );
    assert_eq!(listing.children[Path::new("")][0], Path::new("top"));
}

#[test]
fn listing_rejects_traversal_conflicts_and_excessive_expansion() {
    for name in ["../escape", "/absolute", "dir/../escape", ".", "", "a\0b"] {
        let text = format!("-rw-r--r-- 1 0 0 1 Jan 1 2025 {name}\n");
        assert!(
            Listing::parse(text.as_bytes(), &cancellation()).is_err(),
            "{name:?}"
        );
    }
    for names in [["a", "a/b"], ["a/b", "a"], ["a/b", "./a/b"]] {
        let text: String = names
            .iter()
            .map(|name| format!("-rw-r--r-- 1 0 0 1 Jan 1 2025 {name}\n"))
            .collect();
        assert!(Listing::parse(text.as_bytes(), &cancellation()).is_err());
    }
    let name = vec!["a"; 257].join("/");
    assert!(
        Listing::parse(
            format!("-rw-r--r-- 1 0 0 1 Jan 1 2025 {name}\n").as_bytes(),
            &cancellation()
        )
        .unwrap_err_message()
        .contains("256")
    );
    let text: String = (0..3000)
        .map(|index| {
            format!(
                "-rw-r--r-- 1 0 0 1 Jan 1 2025 {index}{}\n",
                "x".repeat(3000)
            )
        })
        .collect();
    assert!(
        Listing::parse(text.as_bytes(), &cancellation())
            .unwrap_err_message()
            .contains("64 MiB")
    );
}

// Avoid requiring Debug on the immutable listing just for error assertions.
trait ErrorMessage {
    fn unwrap_err_message(self) -> String;
}
impl<T> ErrorMessage for io::Result<T> {
    fn unwrap_err_message(self) -> String {
        match self {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }
}

#[test]
fn helper_lists_reads_refreshes_and_protects_its_native_container() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("container with spaces;$(false)");
    fs::write(&source, b"payload").unwrap();
    let mount = open(
        fixture(dir.path()),
        Some(VfsPath::Local(source.clone())),
        VfsPath::Local(dir.path().into()),
        None,
    )
    .unwrap();
    let root = mount.root();
    let entries = root
        .read_directory(SortMode::default(), true, None)
        .unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "folder" && entry.size == 7)
    );
    assert_eq!(read(&member(&root)).unwrap(), b"payload");
    assert_eq!(root.protected_local_paths(), vec![source.clone()]);
    fs::write(&source, b"new payload").unwrap();
    assert_eq!(read(&member(&root)).unwrap(), b"new payload");
    assert!(root.native_path().is_none());
    mount.close();
    assert_eq!(
        read(&member(&root)).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
}

#[test]
fn standalone_and_nested_archive_helpers_use_the_mc_argument_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let standalone = helper(
        dir.path(),
        "standalone+",
        r#"
case "$1" in
list) test "$#" = 1; printf '%s\n' '-rw-r--r-- 1 0 0 3 Jan 1 2025 folder/file with spaces';;
copyout) test "$#" = 3; printf abc > "$3";;
esac
"#,
    );
    let mount = open(standalone, None, VfsPath::Local(dir.path().into()), None).unwrap();
    assert_eq!(read(&member(&mount.root())).unwrap(), b"abc");

    let archive = dir.path().join("outer.tar");
    let mut builder = tar::Builder::new(File::create(&archive).unwrap());
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o600);
    header.set_size(7);
    header.set_cksum();
    builder
        .append_data(&mut header, "inner.pkg", &b"payload"[..])
        .unwrap();
    builder.finish().unwrap();
    let location = ArchiveLocation::root(archive.clone());
    let source = VfsPath::Archive {
        location: location.clone(),
        path: archive.join("inner.pkg"),
    };
    let nested = open(fixture(dir.path()), Some(source), location.into(), None).unwrap();
    assert_eq!(read(&member(&nested.root())).unwrap(), b"payload");
    assert_eq!(nested.root().protected_local_paths(), vec![archive]);
}

#[test]
fn copyout_rejects_symlinks_truncation_and_writes_past_the_reserved_limit() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::write(&source, b"safe").unwrap();
    for (name, action) in [
        ("symlink", "rm \"$4\"; ln -s \"$2\" \"$4\""),
        ("truncated", "printf x > \"$4\""),
        ("fifo", "rm \"$4\"; mkfifo \"$4\""),
        ("directory", "rm \"$4\"; mkdir \"$4\""),
        ("swallowed", "(printf abcdefgh > \"$4\") || true"),
        ("oversize", "printf abcdefgh > \"$4\"; printf safe > \"$4\""),
    ] {
        let script = format!(
            "case \"$1\" in\nlist) printf '%s\\n' '-rw-r--r-- 1 0 0 4 Jan 1 2025 folder/file with spaces';;\ncopyout) {action};;\nesac"
        );
        let mount = open(
            helper(dir.path(), name, &script),
            Some(VfsPath::Local(source.clone())),
            VfsPath::Local(dir.path().into()),
            None,
        )
        .unwrap();
        assert!(read(&member(&mount.root())).is_err(), "{name}");
    }
    assert_eq!(fs::read(&source).unwrap(), b"safe");
}

#[test]
fn discovery_obeys_precedence_and_validates_executables() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let expected = fixture(first.path());
    fixture(second.path());
    fs::write(first.path().join("notexec"), b"text").unwrap();
    symlink(expected.path(), first.path().join("alias+")).unwrap();
    let found = discover_in([first.path().into(), second.path().into()]);
    assert_eq!(found.len(), 2);
    assert!(!found[0].needs_archive());
    assert_eq!(found[1].path(), expected.path());
}

#[test]
fn unknown_sizes_are_bounded_and_reads_report_actual_snapshot_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let generated = helper(
        dir.path(),
        "package+",
        r#"
case "$1" in
list) printf '%s\n' '-rw-r--r-- 1 0 0 0 Jan 1 2025 INFO/CONTENTS';;
copyout) printf 'generated package contents' > "$3";;
esac
"#,
    );
    let mount = open(generated, None, VfsPath::Local(dir.path().into()), None).unwrap();
    let root = mount.root();
    let file = root
        .resolve_identity(root.identity().join("INFO/CONTENTS"))
        .unwrap();
    file.with_file(None, &mut |entry, reader| {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"generated package contents");
        assert_eq!(entry.size, bytes.len() as u64);
        Ok(())
    })
    .unwrap();
}

#[test]
fn manual_invalidation_reloads_standalone_helper_metadata_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = helper(dir.path(), "catalog+", "cat \"$0.listing\"");
    let listing = dir.path().join("catalog+.listing");
    fs::write(&listing, b"-rw-r--r-- 1 0 0 4 Jan 1 2025 before\n").unwrap();
    let mount = open(catalog, None, VfsPath::Local(dir.path().into()), None).unwrap();
    fs::write(&listing, b"-rw-r--r-- 1 0 0 4 Jan 1 2025 after\n").unwrap();
    let entries = || {
        mount
            .root()
            .read_directory(SortMode::default(), true, None)
            .unwrap()
    };
    assert!(entries().iter().any(|entry| entry.name == "before"));
    mount.invalidate();
    assert!(entries().iter().any(|entry| entry.name == "after"));
}

#[test]
fn helper_cancellation_and_process_limits_do_not_leave_workers_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let slow = helper(dir.path(), "slow+", "sleep 60");
    let canceled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let job = scope.spawn(|| {
            open(
                slow,
                None,
                VfsPath::Local(dir.path().into()),
                Some(&canceled),
            )
        });
        std::thread::sleep(Duration::from_millis(50));
        let start = Instant::now();
        canceled.store(true, Ordering::Relaxed);
        assert_eq!(
            job.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    });
    let mut noisy = Command::new("/bin/sh");
    noisy.arg("-c").arg("while :; do printf abcdefgh; done");
    assert!(process::run(noisy, &[], 100, Duration::from_secs(2), &cancellation()).is_err());
    let mut timed = Command::new("/bin/sh");
    timed.arg("-c").arg("sleep 60");
    assert_eq!(
        process::run(timed, &[], 100, Duration::from_millis(30), &cancellation())
            .unwrap_err_message(),
        "VFS helper timed out"
    );
    let mut inherited = Command::new("/bin/sh");
    inherited.arg("-c").arg("sleep 60 & exit 0");
    assert!(
        process::run(inherited, &[], 100, Duration::from_secs(2), &cancellation())
            .unwrap()
            .status
            .success()
    );
}
